=====
blkio
=====
------------------------
Block device I/O library
------------------------
:Manual section: 3

DESCRIPTION
-----------
libblkio is a library for accessing data stored on *block devices*. Block
devices offer persistent data storage and are addressable in fixed-size units
called *blocks*. Block sizes of 4 KiB or 512 bytes are typical. Hard disk
drives, solid state disks (SSDs), USB mass storage devices, and other types of
hardware are block devices.

The focus of libblkio is on fast I/O for multi-threaded applications.
Management of block devices, including partitioning and resizing, is outside
the scope of the library.

Block devices have one or more *queues* for submitting I/O requests such as
reads and writes. Block devices process I/O requests from their queues and
produce a return code for each completed request indicating success or an
error.

The application is responsible for thread-safety. No thread synchronization is
necessary when a queue is only used from a single thread. Proper
synchronization is required when sharing a queue between multiple threads.

libblkio can be used in blocking, event-driven, and polling modes depending on
the architecture of the application and its performance requirements.

*Blocking mode* suspends the execution of the current thread until the request
completes. This is most natural way of writing programs that perform a sequence
of I/O requests but cannot exploit request parallelism.

*Event-driven mode* provides a completion file descriptor that the application
can monitor from its event loop. This allows multiple I/O requests to be in
flight simultaneously and the application can respond to other events while
waiting for completions.

*Polling mode* also supports multiple in-flight requests but the application
continuously checks for completions, typically from a tight loop, in order to
minimize latency.

libblkio contains *drivers* for several block I/O interfaces. This allows
applications using libblkio to access different block devices through a single
API.

Creating a `blkio` instance
~~~~~~~~~~~~~~~~~~~~~~~~~~~
A `struct blkio` instance is created from a specific driver such as "io_uring"
as follows::

  struct blkio *b;
  int ret;

  ret = blkio_create("io_uring", &b);
  if (ret < 0) {
      fprintf(stderr, "%s: %s\n", strerror(-ret), blkio_get_error_msg());
      return;
  }

For a list of available drivers, see the DRIVERS_ section below.

Error messages
~~~~~~~~~~~~~~
Functions generally return 0 on success and a negative `errno(3)` value on
failure. In the later case, a per-thread error message is also set and can be
obtained as a `const char *` by calling `blkio_get_error_msg()`.

Note that these messages are not stable and may change in between
backward-compatible libblkio releases. The same applies to returned errno
values, unless a specific value is explicitly documented for a particular error
condition.

Connecting to a block device
~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Connection details for a block device are specified by setting properties on
the `blkio` instance. The available properties depend on the driver. For
example, the io_uring driver's "path" property is set to `/dev/sdb` to access a
local disk::

  int ret = blkio_set_str(b, "path", "/dev/sdb");
  if (ret < 0) {
      fprintf(stderr, "%s: %s\n", strerror(-ret), blkio_get_error_msg());
      blkio_destroy(&b);
      return;
  }

Once the connection details have been specified the `blkio` instance can be
connected to the block device with `blkio_connect()`::

  ret = blkio_connect(b);

Starting a block device
~~~~~~~~~~~~~~~~~~~~~~~
After the `blkio` instance is connected, properties are available to configure
its operation and query device characteristics such as the maximum number of
queues. See PROPERTIES_ for details.

For example, the number of queues can be set as follows::

  ret = blkio_set_int(b, "num-queues", 4);

Once configuration is complete the `blkio` instance is started with `blkio_start()`::

  ret = blkio_start(b);

Mapping memory regions
~~~~~~~~~~~~~~~~~~~~~~
Memory containing I/O data buffers must be "mapped" before submitting
requests that touch the memory when the "needs-mem-regions" property is true.
Otherwise mapping memory is optional but doing so may improve performance.

Memory regions are mapped globally for the `blkio` instance and are
available to all queues. A memory region is represented as follows::

  struct blkio_mem_region
  {
      void *addr;
      uint64_t iova;
      size_t len;
      int64_t fd_offset;
      int fd;
      uint32_t flags;
  };

The `addr` field contains the starting address of the memory region. Requests
transfer data between the block device and a subset of the memory region,
including up to the entire memory region. Individual read/write requests or
readv/writev request segments (iovecs) must not access more than one memory
region. Multiple requests can access the same memory region simultaneously,
although usually with non-overlapping areas.

The `addr` field must be a multiple of the "mem-region-alignment" property.

The `iova` field is reserved and must be zero.

The `len` field is the size of the memory region in bytes. The value must be a
multiple of the "mem-region-alignment" property.

The `fd` field is the file descriptor for the memory region. Some drivers
require that I/O data buffers are located in file-backed memory. This can be
anonymous memory from `memfd_create(2)` rather than an actual file on disk.
If the "needs-mem-region-fd" property is true then this field must be a valid
file descriptor. If the property is false this field may be -1.

The `fd_offset` field is the byte offset from the start of the file given in
`fd`.

The `flags` field is reserved and must be zero.

The application can either allocate I/O data buffers itself and describe them
with `struct blkio_mem_region` or it can use `blkio_alloc_mem_region()` and
`blkio_free_mem_region()` to allocate memory suitable for I/O data buffers::

  int blkio_alloc_mem_region(struct blkio *b, struct blkio_mem_region *region,
                             size_t len);
  void blkio_free_mem_region(struct blkio *b,
                             const struct blkio_mem_region *region);

The `len` argument is the number of bytes to allocate. These functions may only
be called after the `blkio` instance has been started.

File descriptors for memory regions created with `blkio_alloc_mem_region()` are
automatically closed across `execve(2)`.

Memory regions can be mapped and unmapped after the `blkio` instance has been
started using the `blkio_map_mem_region()` and `blkio_unmap_mem_region()`
functions::

  int blkio_map_mem_region(struct blkio *b,
                           const struct blkio_mem_region *region);
  void blkio_unmap_mem_region(struct blkio *b,
                              const struct blkio_mem_region *region);

These functions must not be called while requests are in flight that access the
affected memory region. Memory regions must not overlap. Memory regions must be
unmapped/freed with exactly the same `region` field values that they were
mapped/allocated with.

`blkio_map_mem_region()` does not take ownership of `region->fd`. The caller
may close `region->fd` after `blkio_map_mem_region()` returns.

`blkio_map_mem_region()` returns an error if called on a memory region that is
already mapped against the given `blkio`. `blkio_unmap_mem_region()` has no
effect when called on a memory region that is not mapped against the given
`blkio`.

`blkio_free_mem_region()` must not be called on a memory region that was mapped
but not unmapped.

For best performance applications should map memory regions once and reuse them
instead of changing memory regions frequently.

The "max-mem-regions" property gives the maximum number of memory regions that
can be mapped.

Memory regions are automatically unmapped when `blkio_destroy()` is called, and
memory regions allocated using `blkio_alloc_mem_region()` are freed.

Performing I/O
~~~~~~~~~~~~~~
Once at least one memory region has been mapped, the queues are ready for
request processing. The following example reads 4096 bytes from byte offset
0x10000::

  struct blkioq *q = blkio_get_queue(b, 0);

  blkioq_read(q, 0x10000, buf, buf_size, NULL, 0);

  struct blkio_completion completion;
  ret = blkioq_do_io(q, &completion, 1, 1, NULL);
  if (ret != 1) ...
  if (completion.ret != 0) ...

This is an example of blocking mode where `blkioq_do_io()` waits until the I/O
request completes. See below for details on event-driven and polling modes.

The `blkioq_do_io()` function offers the following arguments::

  int blkioq_do_io(struct blkioq *q,
                   struct blkio_completion *completions,
                   int min_completions,
                   int max_completions,
                   struct timespec *timeout);

The `completions` argument is a pointer to an array that is filled in with
completions when the function returns. When `max_completions` is 0
`completions` may be NULL. Completions are represented by `struct
blkio_completion`::

  struct blkio_completion
  {
      void *user_data;
      const char *error_msg;
      int ret;
      /* reserved space */
  };

The `user_data` field is the same pointer passed to `blkioq_read()` in the
example above. Applications that submit multiple requests can use `user_data`
to correlate completions to previously submitted requests.

The `ret` field is the return code for the I/O request in negative errno
representation. This field is 0 on success.

For some errors, the `error_msg` field points to a message describing what
caused the request to fail. Note that this may be `NULL` even if `ret` is not 0,
and is always `NULL` when `ret` is 0.

Note that these messages are not stable and may change in between
backward-compatible libblkio releases. The same applies to the errno values
returned through `ret`, unless a specific value is explicitly documented for a
particular error condition.

`struct blkio_completion` also includes some reserved space which may be used to
add more fields in the future in a backward-compatible manner.

The remaining arguments of `blkioq_do_io()` are as follows:

The `min_completions` argument controls how many completions to wait for. A
value greater than 0 causes the function to block until the number of
completions has been reached. A value of 0 causes the function to submit I/O
and return completions that have already occurred without waiting for more. If
greater than the number of currently outstanding requests, `blkioq_do_io()`
fails with -EINVAL.

The `max_completions` argument is the maximum number of `completions` elements
to fill in. This value must be greater or equal to `min_completions`.

The `timeout` argument specifies the maximum amount of time to wait for
completions. The function returns -ETIME if the timeout expires before a
request completes. If `timeout` is NULL the function blocks indefinitely. When
`timeout` is non-NULL the elapsed time is subtracted and the `struct timespec`
is updated when the function returns regardless of success or failure.

The return value is the number of `completions` elements filled in. This value
is within the inclusive range [`min_completions`, `max_completions`] on success
or a negative errno on failure.

A `blkioq_do_io_interruptible()` variant is also available::

  int blkioq_do_io_interruptible(struct blkioq *q,
                                 struct blkio_completion *completions,
                                 int min_completions,
                                 int max_completions,
                                 struct timespec *timeout,
                                 const sigset_t *sig);

Unlike `blkioq_do_io()`, this function can be interrupted by signals and return
-EINTR. The `sig` argument temporarily sets the signal mask of the process
while waiting for completions, which allows the thread to be woken by a signal
without race conditions. To ensure this function is interrupted when a signal
is received, (1) the said signal must be in a blocked state when invoking the
function (see `sigprocmask(2)`) and (2) a signal mask unblocking that signal
must be given as the `sig` argument.

Event-driven mode
~~~~~~~~~~~~~~~~~
Completion processing can be integrated into the event loop of an application
so that other activity can take place while I/O is in flight. Each queue has a
completion file descriptor that is returned by the following function::

  int blkioq_get_completion_fd(struct blkioq *q);

The returned file descriptor becomes readable when `blkioq_do_io()` needs to be
called again. Spurious events can occur, causing the fd to become readable even
if there are no new completions available.

The returned file descriptor has O_NONBLOCK set. The application may switch the
file descriptor to blocking mode.

By default, the driver might not generate completion events for requests so it
is necessary to explicitly enable the completion file descriptor before use::

  void blkioq_set_completion_fd_enabled(struct blkioq *q, bool enable);

Changes made using this function apply also to requests that are already in
flight but not yet completed. Note that even after calling this function with
`enabled` as `false`, the driver may still generate completion events.

The application must read 8 bytes from the completion file descriptor to reset
the event before calling `blkioq_do_io()`. The contents of the bytes are
undefined and should not be interpreted by the application.

The following example demonstrates event-driven I/O::

  struct blkioq *q = blkio_get_queue(b, 0);
  int completion_fd = blkio_get_completion_fd(q);
  char event_data[8];

  /* Switch to blocking mode for read(2) below */
  fcntl(completion_fd, F_SETFL,
        fcntl(completion_fd, F_GETFL, NULL) & ~O_NONBLOCK);

  /* Enable completion events */
  blkioq_set_completion_fd_enabled(q, true);

  blkioq_read(q, 0x10000, buf, buf_size, NULL, 0);

  /* Since min_completions = 0 we will submit but not wait */
  ret = blkioq_do_io(q, NULL, 0, 0, NULL);
  if (ret != 0) ...

  /* Wait for the next event on the completion file descriptor */
  struct blkio_completion completion;
  do {
    read(completion_fd, event_data, sizeof(event_data));
    ret = blkioq_do_io(q, &completion, 0, 1, NULL);
  } while (ret == 0);
  if (ret != 1) ...
  if (completion.ret != 0) ...

This example uses a blocking `read(2)` to wait and consume the next event on the
completion file descriptor. Because spurious events can occur, it then checks if
there actually is a completion available, retrying `read(2)` otherwise.

Normally `completion_fd` would be registered with an event loop so the
application can perform other tasks while waiting.

Applications may save CPU cycles by suppressing completion file descriptor
notifications while processing completions. This optimization avoids an
unnecessary application event loop iteration and completion file descriptor
read when additional completions arrive while the application is processing
completions::

  static void process_completions(...)
  {
      int ret;

      /* Supress completion fd notifications while we process completions */
      blkioq_set_completion_fd_enabled(q, false);

      do {
          struct blkioq_completion completion;
          ret = blkioq_do_io(q, &completion, 0, 1, NULL);

          if (ret == 0) {
              blkioq_set_completion_fd_enabled(q, true);

              /* Re-check for completions to avoid race */
              ret = blkioq_do_io(q, &completion, 0, 1, NULL);
              if (ret == 1) {
                  blkioq_set_completion_fd_enabled(q, false);
              }
          }

          if (ret < 0) {
              ... /* error */
          }

          if (ret == 1) {
              ... /* process completion */
          }
      } while (ret == 1);
  }

Application-level polling mode
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Waiting for completions using `blkioq_do_io()` with `min_completions` > 0 can
cause the current thread to be descheduled by the operating system's scheduler.
The same is true when waiting for events on the completion file descriptor
returned by `blkioq_get_completion_fd()`. Some applications require consistent
low response times and therefore cannot risk being descheduled.

`blkioq_do_io()` may be called from a CPU polling loop with `min_completions` =
0 to check for completions::

  struct blkioq *q = blkio_get_queue(b, 0);

  blkioq_read(q, 0x10000, buf, buf_size, NULL, 0);

  /* Busy-wait for the completion */
  struct blkio_completion completion;
  do {
      ret = blkioq_do_io(q, &completion, 0, 1, NULL);
  } while (ret == 0);

  if (ret != 1) ...
  if (completion.ret != 0) ...

This approach is ideal for applications that need to poll several event sources
simultaneously, or that need to intersperse polling with other application
logic. Otherwise, driver-level polling (see below) may lead to further
performance gains.

Driver-level polling mode (poll queues)
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Poll queues differ from the "regular" queues presented above in that calling
`blkioq_do_io()` with `min_completions` > 0 causes libblkio itself (or other
lower layers) to poll for completions. This can be more efficient than
repeatedly invoking `blkioq_do_io()` with `min_completions` = 0 on a "regular"
queue. For instance, with the io_uring driver, poll queues cause the kernel
itself to poll for completions, avoiding repeated context switching while
polling.

A limitation of poll queues is that the CPU thread is occupied with a single
poll queue and cannot detect other events in the meantime such as network I/O or
application events. Applications wishing to poll multiple things simultaneously
may prefer to use application-level polling (see above).

Poll queue support is contingent on the particular driver and driver
configuration being used. To determine whether a given `blkio` supports poll
queues, check the "supports-poll-queues" property::

  bool supports_poll_queues;
  ret = blkio_get_bool(b, "supports-poll-queues", &supports_poll_queues);
  if (ret != 0) ...

  if (!supports_poll_queues) {
      fprintf(stderr, "Poll queues not supported\n");
      return;
  }

It is possible for poll queues not to support flush, write zeroes, and discard
requests, even if "regular" queues of the same `blkio` do. However, read, write,
readv, and writev requests are always supported. There is currently no mechanism
to check which types of requests are supported by poll queues.

To use poll queues, set the "num-poll-queues" property to a positive value
before calling `blkio_start()`, then use `blkio_get_poll_queue()` to retrieve
the poll queues. A single `blkio` can have both "regular" queues and poll
queues::

  ...
  ret = blkio_connect(b);
  if (ret != 0) ...

  ret = blkio_set_int(b, "num-queues", 1);
  ret = blkio_set_int(b, "num-poll-queues", 1);
  if (ret != 0) ...

  ret = blkio_start(b);
  if (ret != 0) ...

  struct blkioq *q      = blkio_get_queue(b, 0);
  struct blkioq *poll_q = blkio_get_poll_queue(b, 0);

It is possible to set property "num-queues" to 0 as long as "num-poll-queues" is
positive.

Poll queues also differ from "regular" queues in that they do not have a
completion fd. `blkioq_get_completion_fd()` returns -1 when called on a poll
queue, and `blkioq_set_completion_fd_enabled()` has no effect. Further,
`blkioq_do_io_interruptible()` is not currently supported on poll queues.

Note that you can still perform application-level polling on poll queues by
repeatedly calling `blkioq_do_io()` with `min_completions` = 0, but this will
lead to suboptimal performance.

Dynamically adding and removing queues
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Some drivers have support for adding queues on demand after the `blkio` instance
is already started::

  int index = blkio_add_queue(b); /* or blkio_add_poll_queue() */
  if (ret < 0) ...

  struct blkioq *q = blkio_get_queue(b, index); /* or blkio_get_poll_queue() */

The "can-add-queues" property determines whether this is supported. When it is,
the `blkio` instance can be started with 0 queues.

In addition, *all* drivers allow explicitly removing queues, regardless of
whether those queues were created by `blkio_start()` or `blkio_add_queue()` /
`blkio_add_poll_queue()`::

  assert(blkio_get_queue(b, 0) != NULL);
  assert(blkio_get_queue(b, 1) != NULL);

  /* blkio_remove_queue() will return 0, indicating success */
  assert(blkio_remove_queue(b, 0) == 0);

  /* Other queues' indices are not shifted, so q will be non-NULL and valid */
  struct blkio *q = blkio_get_queue(b, 1);
  assert(q != NULL);

  /* blkio_remove_queue() will return -ENOENT, since queue 0 no longer exists */
  assert(blkio_remove_queue(b, 0) == -ENOENT);

Once a queue is removed, any `struct blkioq *` pointing to it becomes invalid.

Request types
~~~~~~~~~~~~~
The following types of I/O requests are available::

  void blkioq_read(struct blkioq *q, uint64_t start, void *buf, size_t len,
                   void *user_data, uint32_t flags);
  void blkioq_write(struct blkioq *q, uint64_t start, void *buf, size_t len,
                    void *user_data, uint32_t flags);
  void blkioq_readv(struct blkioq *q, uint64_t start, struct iovec *iovec,
                    int iovcnt, void *user_data, uint32_t flags);
  void blkioq_writev(struct blkioq *q, uint64_t start, struct iovec *iovec,
                     int iovcnt, void *user_data, uint32_t flags);
  void blkioq_write_zeroes(struct blkioq *q, uint64_t start, uint64_t len,
                           void *user_data, uint32_t flags);
  void blkioq_discard(struct blkioq *q, uint64_t start, uint64_t len,
                      void *user_data, uint32_t flags);
  void blkioq_flush(struct blkioq *q, void *user_data, uint32_t flags);

The block device may see requests as soon as they these functions are called,
but `blkioq_do_io()` must be called to ensure requests are seen.

If property "needs-mem-regions" is true, I/O data buffers pointed to by `buf`
and `iovec` must be within regions mapped using `blkio_map_mem_region()`.

The application must not free the `iovec` elements until the request's
completion is returned by `blkioq_do_io()`.

All drivers are guaranteed to support at least `blkioq_read()`,
`blkioq_write()`, `blkioq_readv()`, `blkioq_writev()`, and `blkioq_flush()`.
When attempting to queue a request that the driver does not support, the
request itself fails and its completion's `ret` field is -ENOTSUP.

`blkioq_read()` and `blkioq_readv()` read data from the block device at byte
offset `start`. `blkioq_write()` and `blkioq_writev()` write data to the block
device at byte offset `start`. The length of the I/O data buffer is `len` bytes
and the total size of the `iovec` elements, respectively. `start` and the
length of the I/O data buffer must be a multiple of the "request-alignment"
property. I/O data buffer addresses and lengths, including `buf` and individual
`iovec` elements, must be multiples of the "buf-alignment" property.

`blkioq_write_zeroes()` causes zeros to be written to the specified region. When
supported, this may be more efficient than using `blkioq_write()` with a
zero-filled buffer.

`blkioq_discard()` causes data in the specified region to be discarded.
Subsequent reads to the same region return unspecified data until it is written
to again. Note that discarded data is not guaranteed to be erased and may still
be returned by reads.

`blkioq_flush()` persists completed writes to the storage medium. Data is
persistent once the flush request completes successfully. Applications that
need to ensure that data persists across power failure or crash must submit
flush requests at appropriate points.

The `user_data` pointer is returned in the `struct blkio_completion::user_data`
field by `blkioq_do_io()`. It allows applications to correlate a completion
with its request.

No ordering guarantees are defined for requests that are in flight
simultaneously. For example, a flush request is not guaranteed to persist
in-flight write requests. Instead the application must wait for write requests
that it wishes to persist to complete before calling `blkioq_flush()`.

Similarly, there are no ordering guarantees between multiple queues of a block
device. Multi-threaded applications that rely on an ordering between multiple
queues must wait for the first request to complete on one queue, synchronize
threads as needed, and then submit the second request on the other queue.

Request flags
`````````````
The following request flags are available:

BLKIO_REQ_FUA
  Ensures that data written by this request reaches persistent storage before
  the request is completed. This is also known as Full Unit Access (FUA). This
  flag eliminates the need for a separate `blkioq_flush()` call after the
  request has completed. Other data that was previously successfully written
  without the `BLKIO_REQ_FUA` flag is not necessarily persisted by this flag as
  it is only guaranteed to affect the current request. Supported by
  `blkioq_write()`, `blkioq_writev()`, and `blkioq_write_zeroes()`.

BLKIO_REQ_NO_UNMAP
  Ensures that `blkioq_write_zeroes()` does not cause underlying storage space
  to be deallocated, guaranteeing that subsequent writes to the same region do
  not fail due to lack of space.

BLKIO_REQ_NO_FALLBACK
  Ensures that `blkioq_write_zeroes()` does not resort to performing regular
  write requests with zero-filled buffers. If that would otherwise be the case
  and this flag is set, then the request fails and its completion's `ret` field
  is -ENOTSUP.

PROPERTIES
----------
The configuration of `blkio` instances is done through property accesses. Each
property has a name and a type (bool, int, str, uint64). Properties may be
read-only (r), write-only (w), or read/write (rw).

Access to properties depends on the `blkio` instance state
(created/connected/started). A property may be read/write in the connected
state but read-only in the started state. This is written as "rw connected, r
started".

The following properties APIs are available::

  int blkio_get_bool(struct blkio *b, const char *name, bool *value);
  int blkio_get_int(struct blkio *b, const char *name, int *value);
  int blkio_get_uint64(struct blkio *b, const char *name, uint64_t *value);
  int blkio_get_str(struct blkio *b, const char *name, char **value);

  int blkio_set_bool(struct blkio *b, const char *name, bool value);
  int blkio_set_int(struct blkio *b, const char *name, int value);
  int blkio_set_uint64(struct blkio *b, const char *name, uint64_t value);
  int blkio_set_str(struct blkio *b, const char *name, const char *value);

`blkio_get_str()` assigns to `*value` and the caller must use `free(3)` to
deallocate the memory.

`blkio_get_str()` automatically converts to string representation if the
property is not a str. `blkio_set_str()` automatically converts from string
representation if the property is not a str. This can be used to easily fetch
values from and store values to an application's text-based configuration file
or command-line. Aside from this automatic conversion, the other property APIs
fail with ENOTTY if the property does not have the right type.

The following properties are common across all drivers. Driver-specific
properties are documented in DRIVERS_.

Properties available after `blkio_create()`
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

can-add-queues (bool, r created/connected/started)
  Whether the driver supports dynamically adding queues with `blkio_add_queue()`
  / `blkio_add_poll_queue()`.

driver (str, r created/connected/started)
  The driver name that was passed to `blkio_create()`. See DRIVERS_ for details
  on available drivers.

read-only (bool, rw created, r connected/started)
  If true, requests other than read and flush fail with -EBADF. The default is
  false.

Properties available after `blkio_connect()`
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

**DEVICE AND QUEUES**

  capacity (uint64, r connected/started)
    The size of the block device in bytes.

  max-queues (int, r connected/started)
    The maximum number of queues, including poll queues if any.

  num-queues (int, rw connected, r started)
    The number of queues. The default is 1.

  num-poll-queues (int, rw connected, r started)
    The number of poll queues. The default is 0. If set to a positive value and
    property "supports-poll-queues" is false, `blkio_start()` will fail.

  supports-poll-queues (bool, r connected/started)
    Whether the driver supports poll queues.

**MEMORY REGIONS**

  max-mem-regions (uint64, r connected/started)
    The maximum number of memory regions that can be mapped at any given time.

  may-pin-mem-regions (bool, r connected/started)
    Will the driver sometimes pin memory region pages and therefore prevent
    madvise(MADV_DONTNEED) and related syscalls from working?

  mem-region-alignment (uint64, r connected/started)
    The alignment requirement, in bytes, for the `addr`, `iova`, and `size` in
    `struct blkio_memory_region`. This is always a multiple of the
    "buf-alignment" property.

  needs-mem-regions (bool, r connected/started)
    Is it necessary to map memory regions with `blkio_map_mem_region()`?

  needs-mem-region-fd (bool, r connected/started)
    Is it necessary to provide a file descriptor for each memory region?

**ALL REQUESTS**

  optimal-io-alignment (int, r connected/started)
    The ideal number of bytes of request start and length alignment for maximizing
    performance. This is a multiple of the "request-alignment" property.

  optimal-io-size (int, r connected/started)
    The ideal request length in bytes for achieving high throughput. Can be 0 if
    unspecified. Otherwise, this is a multiple of the "optimal-io-alignment"
    property.

  request-alignment (int, r connected/started)
    All request start and length must be a multiple of this value. Often this
    value is 512 bytes.

  flush-needed (bool, r, connected/started)
    Whether a flush request must be sent after write request completion to ensure
    data persistence.

**READ AND WRITE REQUESTS**

  buf-alignment (int, r connected/started)
    I/O data buffer memory address and length alignment, including plain `void
    *buf` buffers and iovec segments. Note the "mem-region-alignment" property is
    always a multiple of this value.

  can-grow (bool, r connected/started)
    If false `blkioq_read()`, `blkioq_readv()`, `blkioq_write()` and `blkioq_writev()`
    will fail if an attempt to read/write beyond of EOF is made. Otherwise, reads will
    succeed and the portion of the read buffer that overruns EOF will be filled with zeros,
    and writes will increase the the device's capacity.

  max-segments (int, r connected/started)
    The maximum iovcnt in a request.

  max-segment-len (int, r connected/started)
    The maximum size of each iovec in a request. Can be 0 if unspecified.

  max-transfer (int, r connected/started)
    The maximum read or write request length in bytes. Can be 0 if unspecified.

  optimal-buf-alignment (int, r connected/started)
    The ideal number of bytes of I/O data buffer memory address and length
    alignment, including plain `void *buf` buffers and iovec segments.

  supports-fua-natively (bool, r connected/started)
    Whether `blkioq_write()` and `blkioq_writev()` support the BLKIO_REQ_FUA flag
    natively, as opposed to emulating it by internally performing a flush request
    after the write. This does *not* currently indicate whether
    `blkioq_write_zeroes()` support for BLKIO_REQ_FUA is native or emulated.

**WRITE ZEROES REQUESTS**

  max-write-zeroes-len (uint64, r connected/started)
    The maximum length of a write zeroes request in bytes. Can be 0 if
    unspecified.

**DISCARD REQUESTS**

  discard-alignment (int, r connected/started)
    Discard request start and length, after subtracting the value of the
    "discard-alignment-offset" property, must be a multiple of this value. This
    may or may not be 0 if discard requests are not supported. If not 0, this is a
    multiple of the "request-alignment" property.

  discard-alignment-offset (int, r connected/started)
    Offset of the first block that may be discarded. This may be non-zero, for
    example, when the device is a partition that is not aligned to the value of
    the "discard-alignment" property. This may or may not be 0 if discard requests
    are not supported. If not 0, this is a multiple of the "request-alignment"
    property, and is less than the "discard-alignment" property.

  max-discard-len (uint64, r connected/started)
    The maximum length of a discard request in bytes. Can be 0 if unspecified.

DRIVERS
-------

io_uring
~~~~~~~~
The io_uring driver uses the Linux io_uring system call interface to perform
I/O on files and block device nodes. Both regular files and block device nodes
are supported.

Note that io_uring was introduced in Linux kernel version 5.1, and kernels may
also be configured to disable io_uring. If io_uring is not available,
`blkio_create()` fails with -ENOSYS when using this driver.

When performing I/O on regular files, write zeroes requests that extend past the
end-of-file *may or may not* update the file size. This is left unspecified and
the user must not rely on any particular behavior.

This driver supports poll queues only when using O_DIRECT on block devices or
file systems that support polling. Its poll queues never support flush, write
zeroes, or discard requests.

**Driver-specific properties available after** `blkio_create()`

  direct (bool, rw created, r connected/started)
    True to bypass the page cache with O_DIRECT. The default is false.

  fd (int, rw created, r connected/started)
    An existing open file descriptor for the file or block device node. Ownership
    of the file descriptor is passed to the library when blkio_connect()
    returns success.

    If this property is set, properties "direct" and "read-only" have no effect
    and it is the user's responsibility to open the file with the desired flags.
    Further, during connect, those two properties are updated to reflect the
    file status flags of the given file descriptor.

  path (str, rw created, r connected/started)
    The file system path of the file or block device node.

    If this property is set, property "fd" must not be set and will be updated
    on connect to reflect the opened file descriptor. Note that the file
    descriptor is owned by libblkio.

**Driver-specific properties available after** `blkio_connect()`

  num-entries (int, rw connected, r started)
    The minimum number of entries that each io_uring submission queue and
    completion queue should have. The default is 128.

    A larger value allows more requests to be in flight, but consumes more
    resources. Tuning this value can affect performance.

    io_uring imposes a maximum on this number: 32768 as of mainline kernel 5.18,
    and 4096 prior to 5.4. If this maximum is exceeded, `blkio_start()` will fail
    with -EINVAL.

nvme-io_uring
~~~~~~~~~~~~~
The nvme-io_uring driver submits NVMe commands directly to an NVMe namespace
using io_uring passthrough, which is available since mainline Linux kernel 5.19.

The process must have the CAP_SYS_ADMIN capability to use this driver, and the
NVMe namespace must use the NVM command set.

**Driver-specific properties available after** `blkio_create()`

  fd (int, rw created, r connected/started)
    An existing open file descriptor for the NVMe namespace's character device
    (e.g., `/dev/ng0n1`). Ownership of the file descriptor is passed to the
    library when blkio_connect() returns success.

  path (str, rw created, r connected/started)
    A path to the NVMe namespace's character device (e.g., `/dev/ng0n1`).

    If this property is set, property "fd" must not be set and will be updated
    on connect to reflect the opened file descriptor. Note that the file
    descriptor is owned by libblkio.

**Driver-specific properties available after** `blkio_connect()`

  num-entries (int, rw connected, r started)
    The minimum number of entries that each io_uring submission queue and
    completion queue should have. The default is 128.

    A larger value allows more requests to be in flight, but consumes more
    resources. Tuning this value can affect performance.

    io_uring imposes a maximum on this number: 32768 as of mainline kernel 5.18,
    and 4096 prior to 5.4. If this maximum is exceeded, `blkio_start()` will fail
    with -EINVAL.

  zoned (int, r connected/started)
    - None (0). Zoned storage is not supported.
    - Host-aware (1). Random write requests are supported for backward
      compatibility although zoned storage semantics are supported.
    - Host-managed (2). Only sequential writes are supported according to zoned
      storage semantics.

  max_active_zones (int, r connected/started)
    The number of zones that can be in the implicit open, explicit open, or
    closed state at any given time. This number is always greater or equal to
    the "max_open_zones" property.

    When this number is reached, the application must reset or finish a
    currently active zone in order to free resources for further operations.
    This number only affects the ability to write zones and not the ability to
    read.

  max_open_zones (int, r connected/started)
    The number of zones that can be in the implicit open or explicit open state
    at any given time.

    When this number is reached, the application must close, finish, or reset a
    currently open zone in order to free resources for further operations. This
    number only affects the ability to write zones and not the ability to read.

  zone_size (u64, r connected/started)
    The maximum number of bytes available in each zone.

  nr_zones (u64, r connected/started)
    The number of zones available.

  append_support (bool, r connected/started)
    Whether or not zone append requests are supported.

  zone_append_max_bytes (u64, r connected/started)
    The maximum number of bytes for a zone append request.

virtio-blk-...
~~~~~~~~~~~~~~

The following virtio-blk drivers are provided:

- The virtio-blk-vfio-pci driver uses uses VFIO to control a PCI virtio-blk
  device.

- The virtio-blk-vhost-user driver connects as a client to a Unix domain socket
  provided by a vhost-user-blk backend (e.g. exported from
  `qemu-storage-daemon`).

- The virtio-blk-vhost-vdpa driver uses vhost-vdpa kernel interface to perform
  I/O on a vDPA device. vDPA device could be implemented in software (VDUSE,
  in-kernel, simulator) or in hardware.

These drivers always support poll queues, and their poll queues support all
types of requests.

The following properties apply to all these drivers with some exceptions
described in the property.

**Driver-specific properties available after** `blkio_create()`

  fd (int, rw created, r connected/started)
    An existing open file descriptor for the file system path (see `path` below).
    Ownership of the file descriptor is passed to the library when
    blkio_connect() returns success.
    Currently supported by the following drivers:
    - virtio-blk-vhost-vdpa

  path (str, rw created, r connected/started)
    - virtio-blk-vfio-pci: The file system path of the device's sysfs directory,
      e.g., `/sys/bus/pci/devices/0000:00:01.0`.
    - virtio-blk-vhost-user: The file system path of the vhost-user socket to
      connect to.
    - virtio-blk-vhost-vdpa: The file system path of the vhost-vdpa
      character device to connect to.

**Driver-specific properties available after** `blkio_connect()`

  max-queue-size (int, r connected/started)
    The maximum queue size supported by the device.

  queue-size (int, rw connected, r started)
    The queue size to configure the device with. The default is 256. A larger
    value allows more requests to be in flight, but consumes more resources.
    Tuning this value can affect performance.

BUILD SYSTEM INTEGRATION
------------------------

pkg-config is the recommended way to build a program with libblkio::

  $ cc -o app app.c `pkg-config blkio --cflags --libs`

Meson projects can use pkg-config as follows::

  blkio = dependency('blkio')
  executable('app', 'app.c', dependencies : [blkio])

FREQUENTLY ASKED QUESTIONS
--------------------------
Can network storage drivers be added?
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Maybe. The API was designed with a synchronous control path. Functions like
`blkio_get_uint64()` must return quickly. Operations on network storage can
take an unbounded amount of time (in the absence of a timeout mechanism) and
are not a good fit for synchronous APIs. A more complex asynchronous control
path API could be added for applications wishing to use network storage drivers
in the future.

Can non-Linux operating systems be supported in the future?
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Maybe. No attempt has been made to restrict the library to POSIX features only
and most drivers are platform-specific. If there is demand for supporting other
operating systems and developers willing to work on it then it may be possible.

Can a Linux AIO driver be added?
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Linux AIO could serve as a fallback on systems where io_uring is not available.
However, `io_submit(2)` can block the process and this causes performance
problems in event-driven applications that require that the event loop does not
block. Unless Linux AIO is fixed it is unlikely that a proposal to add a driver
will be accepted.

SEE ALSO
--------
io_uring_setup(2), io_setup(2), aio(7)
