#
# This project provides several test suites:
#
# - "examples"
#   Set of scripts that test our examples with the io_uring driver.
#
# - "generic"
#   Generic tests not related to a specific driver.
#
# - "io_uring"
#   Tests using the io_uring driver on a temporary file.
#
# - "nvme-io_uring"
#   Tests using the nvme-io_uring driver. This test suite requires you to
#   specify the character device corresponding to an NVMe namespace using the
#   --path option. For instance:
#       $ meson test -C build --suite nvme-io_uring \
#           --test-args "--path /dev/ng0n1"
#
# - "virtio-blk-vfio-pci"
#   Tests using the virtio-blk-vfio-pci driver. This test suite requires you to
#   specify a virtio-blk-pci device using the --path option. For instance:
#       $ meson test -C build --suite virtio-blk-vfio-pci \
#           --test-args "--path /sys/bus/pci/devices/0000:00:01.0"
#
# - "virtio-blk-vhost-user"
#   Tests using virtio-blk-vhost-user driver. This test suit uses
#   "/tmp/libblkio-vhost-user-tests.sock" Unix socket path by default.
#   To override it, please use:
#       $ meson test -C build --suite virtio-blk-vhost-user \
#           --test-args "--path /path/to/vhost.sock"
#
#   Example with QSD (QEMU v7.2):
#
#       $ qemu-storage-daemon \
#           --blockdev file,filename=/tmp/libblkio-vhost-user-tests.qcow2,node-name=file \
#           --blockdev qcow2,file=file,node-name=qcow2 \
#           --export type=vhost-user-blk,id=export,addr.type=unix,addr.path=/tmp/libblkio-vhost-user-tests.sock,node-name=qcow2,writable=on,num-queues=2 &
#
#       $ meson test -C build --suite virtio-blk-vhost-user
#
# - "virtio-blk-vhost-vdpa"
#   Tests using virtio-blk-vhost-vdpa driver. This test suit uses
#   "/dev/vhost-vdpa-0" device by default. To override it, please use:
#       $ meson test -C build --suite virtio-blk-vhost-vdpa \
#           --test-args "--path /dev/vhost-vdpa-1"
#
#   Example with the vDPA block simulator (Linux v6.0):
#       $ modprobe vhost-vdpa
#       $ modprobe vdpa-sim-blk
#
#       $ vdpa dev add mgmtdev vdpasim_blk name blk0
#
#       $ meson test -C build --suite virtio-blk-vhost-vdpa
#
#       $ vdpa dev del blk0
#
#   Example with QSD (QEMU v7.2) and VDUSE (Linux v6.0):
#       $ modprobe vhost-vdpa
#       $ modprobe vduse
#
#       $ qemu-storage-daemon \
#           --blockdev file,filename=/tmp/libblkio-vhost-user-tests.qcow2,node-name=file \
#           --blockdev qcow2,file=file,node-name=qcow2 \
#           --export type=vduse-blk,id=vduse0,name=vduse0,node-name=qcow2,writable=on,num-queues=2 &
#
#       $ vdpa dev add name vduse0 mgmtdev vduse
#
#       $ meson test -C build --suite virtio-blk-vhost-vdpa
#
#       $ vdpa dev del vduse0
#

tests = {
  'add-remove-queues': 'add-remove-queues.c',
  'alloc-mem-region': 'alloc-mem-region.c',
  'auto-free-mem-regions': 'auto-free-mem-regions.c',
  'block-limits': 'block-limits.c',
  'capacity': 'capacity.c',
  'discard': 'discard.c',
  'driver_name': 'driver-name.c',
  'driver_not_found': 'driver-not-found.c',
  'error-msg': 'error-msg.c',
  'fd': 'fd.c',
  'flush': 'flush.c',
  'get_completion_fd': 'get-completion-fd.c',
  'get_queue': 'get-queue.c',
  'growing_file': 'growing_file.c',
  'map-mem-region': 'map-mem-region.c',
  'poll-queues': 'poll-queues.c',
  'polled': 'polled.c',
  'queue-full': 'queue-full.c',
  'read': 'read.c',
  'read-invalid-flags': 'read-invalid-flags.c',
  'read-only': 'read-only.c',
  'readv': 'readv.c',
  'read-short': 'read-short.c',
  'set-completion-fd': 'set-completion-fd.c',
  'started': 'started.c',
  'timeout': 'timeout.c',
  'user-data': 'user-data.c',
  'write': 'write.c',
  'write-fua': 'write-fua.c',
  'write-read': 'write-read.c',
  'write-zeroes': 'write-zeroes.c',
  'writev': 'writev.c',
  'writev-readv': 'writev-readv.c',
}

# Most tests have no dependencies so keep this separate from the `tests` dict.
test_dependencies = {
  'error-msg': dependency('threads'),
}

generic_tests = [
  'driver_not_found',
  'error-msg',
]

iouring_tests = [
  'add-remove-queues',
  'alloc-mem-region',
  'auto-free-mem-regions',
  'block-limits',
  'capacity',
  'discard',
  'driver_name',
  'fd',
  'flush',
  'get_completion_fd',
  'get_queue',
  'growing_file',
  'map-mem-region',
  'poll-queues',
  'polled',
  'queue-full',
  'read',
  'read-invalid-flags',
  'read-only',
  'readv',
  'read-short',
  'started',
  'timeout',
  'user-data',
  'write',
  'write-fua',
  'write-read',
  'write-zeroes',
  'writev',
  'writev-readv',
]

examples = [
  'blkio-bench.sh',
  'blkio-copy.sh',
  'blkio-info.sh',
]

nvme_io_uring_tests = [
  'add-remove-queues',
  'alloc-mem-region',
  'auto-free-mem-regions',
  'block-limits',
  'capacity',
  'discard',
  'driver-name',
  'driver-not-found',
  'error-msg',
  'flush',
  'get-completion-fd',
  'get-queue',
  'growing_file',
  'map-mem-region',
  'polled',
  'poll-queues',
  'queue-full',
  'read-invalid-flags',
  'read-only',
  'started',
  'timeout',
  'user-data',
  'write-fua',
  'write-read',
  'writev-readv',
  'write-zeroes',
]

nvme_zoned_script_tests = [
  'props-blkio-info.sh',
]

virtio_blk_tests = [
  'add-remove-queues',
  'alloc-mem-region',
  'auto-free-mem-regions',
  'discard',
  'driver_name',
  'fd',
  'flush',
  'get_completion_fd',
  'get_queue',
  'map-mem-region',
  'poll-queues',
  'polled',
  'queue-full',
  'read-invalid-flags',
  'read-only',
  'set-completion-fd',
  'started',
  'timeout',
  'user-data',
  'write-fua',
  'write-read',
  'write-zeroes',
  'writev-readv',
]

foreach name, c_file : tests
  exe = executable(name,
                   [c_file, 'util.c'],
                   dependencies : [libblkio_dep, test_dependencies.get(name, [])])

  if name in generic_tests
    test(name, exe, suite: 'generic')
  endif

  if name in iouring_tests
    # io_uring tests can't go in parallel because they can interfere with
    # eachother when writing data to the same pre-existing file or device when
    # one is given using the --path option. Lower the priority to make the other
    # tests go in parallel.
    test(name, exe, args : ['--driver', 'io_uring'],
         is_parallel : false, priority: -10, suite: 'io_uring')

    # But provide a parallel version of the suite for when --path isn't used.
    test(name, exe, args : ['--driver', 'io_uring'], suite: 'io_uring+parallel')
  endif

  if name in nvme_io_uring_tests
    # nvme-io_uring tests can't go in parallel because they can interfere with
    # eachother when writing data to the same device. Lower the priority to make
    # the other tests go in parallel.
    test(name, exe, args : ['--driver', 'nvme-io_uring'],
        is_parallel : false, priority: -10, suite: 'nvme-io_uring')
  endif

  if name in virtio_blk_tests
    # virtio-blk tests can't go in parallel because the device
    # can only be used by one process at a time.
    # Lower the priority to make the other tests go in parallel.
    test(name, exe,
         args : ['--driver', 'virtio-blk-vfio-pci'],
         is_parallel : false, priority: -10, suite: 'virtio-blk-vfio-pci')

    test(name, exe,
         args : ['--driver', 'virtio-blk-vhost-vdpa',
                 '--path', '/dev/vhost-vdpa-0'],
         is_parallel : false, priority: -10, suite: 'virtio-blk-vhost-vdpa')

    test(name, exe,
         args : ['--driver', 'virtio-blk-vhost-user',
                 '--path', '/tmp/libblkio-vhost-user-tests.sock'],
         is_parallel : false, priority: -10, suite: 'virtio-blk-vhost-user')
  endif
endforeach

foreach name : examples
  exe = find_program(name)

  test(name, exe, suite: 'examples')
endforeach

foreach name : nvme_zoned_script_tests
  exe = find_program(name)

  test(name, exe, suite: 'nvme-io_uring-zoned')
endforeach
