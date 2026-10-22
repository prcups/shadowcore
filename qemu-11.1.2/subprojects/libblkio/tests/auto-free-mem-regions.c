// SPDX-License-Identifier: (MIT OR Apache-2.0)
#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include "util.h"

enum {
    TEST_FILE_SIZE  = 1024 * 1024,
    MEM_REGION_SIZE = 1024 * 1024,
};

static char filename[] = "auto-free-mem-regions-XXXXXX";

static bool is_valid_fd(int fd)
{
    return fcntl(fd, F_GETFD) != -1 || errno != EBADF;
}

/*
 * Make sure memory regions are automatically freed when blkio_destroy() is
 * called.
 */
int main(int argc, char **argv)
{
    struct test_opts opts;
    struct blkio *b;
    struct blkio_mem_region mem_regions[3];

    parse_generic_opts(&opts, argc, argv);

    create_and_connect(&b, &opts, filename, TEST_FILE_SIZE);

    // allocate 3 memory regions

    for (int i = 0; i < 3; ++i)
        ok(blkio_alloc_mem_region(b, &mem_regions[i], MEM_REGION_SIZE));

    assert(is_valid_fd(mem_regions[0].fd));
    assert(is_valid_fd(mem_regions[1].fd));
    assert(is_valid_fd(mem_regions[2].fd));

    // free one of them

    blkio_free_mem_region(b, &mem_regions[1]);

    assert(is_valid_fd(mem_regions[0].fd));
    assert(!is_valid_fd(mem_regions[1].fd));
    assert(is_valid_fd(mem_regions[2].fd));

    // make sure blkio_destroy() frees the other two

    blkio_destroy(&b);

    assert(!is_valid_fd(mem_regions[0].fd));
    assert(!is_valid_fd(mem_regions[1].fd));
    assert(!is_valid_fd(mem_regions[2].fd));

    return 0;
}
