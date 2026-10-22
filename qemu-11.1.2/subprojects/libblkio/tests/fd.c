// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <fcntl.h>

#include "util.h"

enum {
    TEST_FILE_SIZE = 2 * 1024 * 1024,
};

static char filename[] = "fd-XXXXXX";

/*
 * Verify that the `fd` property works by opening the path set by crate() and
 * passing the file descriptor to the driver.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    char *path = NULL;
    int fd;

    parse_generic_opts(&opts, argc, argv);

    /*
     * Only virtio-blk-vhost-vdpa driver supports `fd` among the virtio-blk-*
     * drivers
     */
    skip_if(driver_is_virtio_blk(opts.driver) &&
            !driver_is_virtio_blk_vhost_vdpa(opts.driver));

    create(&b, &opts, filename, TEST_FILE_SIZE);
    ok(blkio_get_str(b, "path", &path));

    fd = open(path, O_RDWR);
    assert(fd > 0);

    free(path);
    path = NULL;

    ok(blkio_set_int(b, "fd", fd));
    ok(blkio_set_str(b, "path", ""));

    ok(blkio_connect(b));

    /* After connect "fd" and "path" are read-only */
    err(blkio_set_int(b, "fd", fd), -EBUSY);
    err(blkio_set_str(b, "path", ""), -EBUSY);

    ok(blkio_start(b));

    blkio_destroy(&b);

    return 0;
}
